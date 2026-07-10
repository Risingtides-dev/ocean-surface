//! Council stage — native Longhouse topic readout.
//!
//! Renders the daemon's folded council snapshot (`GET /v1/longhouse/topics`,
//! the OCEAN-58 durable fold that survives refresh / late attach) as a
//! document-flow column of topic cards: a federation convenes, members seat,
//! marks land on the board, quorum climbs, a decision resolves. The deck
//! polls the snapshot (~1.5s) while mounted: the daemon does not push
//! longhouse frames to this surface's event stream, and the fold is
//! server-side, so the deck mirrors the snapshot rather than re-folding raw
//! frames. The deck also carries the one write affordance: a convene form
//! that POSTs `/v1/longhouse/convene` and lets the poll surface the topic —
//! no client-side council state is invented; Longhouse stays the authority.

use gloo_net::http::Request;
use leptos::prelude::*;
use serde::Deserialize;
use wasm_bindgen_futures::spawn_local;

use crate::daemon::Daemon;

/// A member seated in a council topic (mirrors the daemon's folded
/// `TopicSnapshot.members`).
#[derive(Clone, Debug, Deserialize, PartialEq)]
struct Member {
    #[serde(default)]
    agent_id: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    role: String,
}

/// A mark posted to the topic blackboard (`proposal` / `evidence` / `endorse`
/// / …). `kind` is a free string so an ocean-os vocabulary addition can't fail
/// the decode.
#[derive(Clone, Debug, Deserialize, PartialEq)]
struct MarkView {
    #[serde(default)]
    mark_id: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    summary: String,
    /// The proposal a non-proposal mark points at (endorse/evidence/oppose).
    #[serde(default)]
    target: Option<String>,
}

/// One proposal's running net weight in the quorum tally.
#[derive(Clone, Debug, Deserialize, PartialEq)]
struct Tally {
    #[serde(default)]
    proposal: String,
    #[serde(default)]
    net_weight: f32,
}

/// A council topic as folded and served by the daemon. Enum-ish fields
/// (`federation`, `trigger`, `state`, member `role`, mark `kind`) are mirrored
/// as plain strings so an ocean-os vocabulary addition can never break the
/// decode; unknown fields (`board_id`, `deadline_ms`, `abort_reason`, …) are
/// ignored by serde.
#[derive(Clone, Debug, Deserialize, PartialEq)]
struct TopicSnapshot {
    #[serde(default)]
    topic_id: String,
    #[serde(default)]
    federation: String,
    #[serde(default)]
    trigger: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    state: String,
    /// Front-runner proposal id, if any.
    #[serde(default)]
    leader: Option<String>,
    /// 0.0–1.0: how close the leader is to crossing the quorum threshold.
    #[serde(default)]
    distance_to_quorum: f32,
    #[serde(default)]
    firekeeper: Option<String>,
    /// Winning proposal id, once converged.
    #[serde(default)]
    decision: Option<String>,
    #[serde(default)]
    members: Vec<Member>,
    #[serde(default)]
    marks: Vec<MarkView>,
    #[serde(default)]
    tallies: Vec<Tally>,
}

#[derive(Debug, Default, Deserialize)]
struct TopicsResponse {
    #[serde(default)]
    topics: Vec<TopicSnapshot>,
}

/// Fetch the durable topics snapshot. Any transport/decode failure yields an
/// empty list (the honest empty state), never a panic.
async fn fetch_topics(base: String) -> Vec<TopicSnapshot> {
    let get_url = format!("{}/v1/longhouse/topics", base.trim_end_matches('/'));
    match Request::get(&get_url).send().await {
        Ok(resp) => match resp.json::<TopicsResponse>().await {
            Ok(r) => r.topics,
            Err(err) => {
                log::warn!("council topics decode error: {err}");
                Vec::new()
            }
        },
        Err(err) => {
            log::warn!("council topics fetch error: {err}");
            Vec::new()
        }
    }
}

/// Convene a council. The daemon handler awaits the WHOLE deliberation
/// (rounds of worker calls — 45s+), so the caller must treat this as
/// fire-and-forget: the ~1.5s topic poll shows `TopicConvened` long before
/// this future resolves. Returns `Err(msg)` only for transport/HTTP failure.
async fn post_convene(base: String, question: String) -> Result<(), String> {
    let url = format!("{}/v1/longhouse/convene", base.trim_end_matches('/'));
    let req = Request::post(&url)
        .json(&serde_json::json!({ "question": question }))
        .map_err(|e| format!("convene encode error: {e}"))?;
    match req.send().await {
        Ok(resp) if resp.ok() => Ok(()),
        Ok(resp) => Err(format!("convene failed (status {})", resp.status())),
        Err(err) => Err(format!("convene request error: {err}")),
    }
}

fn trim_line(text: &str, limit: usize) -> String {
    let t = text.trim();
    if t.chars().count() <= limit {
        t.to_string()
    } else {
        let truncated: String = t.chars().take(limit).collect();
        format!("{}…", truncated.trim_end())
    }
}

/// Short display id for an unresolved uuid (leader/decision/target proposals in
/// demo data don't always map to a proposal mark).
fn short8(id: &str) -> String {
    id.chars().take(8).collect()
}

/// Resolve an agent id to its human label, falling back to a short id.
fn member_label(members: &[Member], id: &str) -> String {
    members
        .iter()
        .find(|m| m.agent_id == id)
        .map(|m| m.label.clone())
        .filter(|l| !l.trim().is_empty())
        .unwrap_or_else(|| short8(id))
}

/// Resolve a proposal id to its proposal-mark summary. Real convene data maps
/// cleanly; demo data degrades to a short id (honest, never fabricated).
fn proposal_label(marks: &[MarkView], id: &str) -> String {
    marks
        .iter()
        .find(|m| m.mark_id == id && m.kind == "proposal")
        .map(|m| trim_line(&m.summary, 48))
        .unwrap_or_else(|| short8(id))
}

fn pct(v: f32) -> String {
    format!("{:.0}%", v.clamp(0.0, 1.0) * 100.0)
}

fn state_class(state: &str) -> &'static str {
    match state {
        "converged" => "is-done",
        "aborted" => "is-error",
        _ => "is-running",
    }
}

#[component]
pub fn CouncilStage(daemon: Daemon) -> impl IntoView {
    let url = daemon.url;
    let topics = RwSignal::new(Vec::<TopicSnapshot>::new());

    // Poll the durable topics snapshot while the deck is mounted. The daemon
    // does NOT push longhouse frames to a plain `/v1/agent/events` subscriber,
    // so there is no reliable client-side event to react to — but the snapshot
    // (`GET /v1/longhouse/topics`) is the authoritative OCEAN-58 fold that
    // survives refresh / late attach. Councils deliberate at human cadence, so
    // a ~1.5s poll is live enough and cheap (small JSON). The loop self-ends
    // when the deck closes: `try_set` hands the value back once `topics` is
    // disposed, which we detect and break on (no signal panic, no on_cleanup).
    let base = url.get_untracked();
    spawn_local(async move {
        loop {
            let fetched = fetch_topics(base.clone()).await;
            if topics.try_set(fetched).is_some() {
                break;
            }
            gloo_timers::future::TimeoutFuture::new(1_500).await;
        }
    });

    // Convene form state. `pending` covers the window between submit and the
    // daemon ACCEPTING the request; the handler awaits the whole council, so
    // the pending flag clears on response OR as soon as the poll shows the
    // topic (whichever lands first, via the same disposal-safe `try_set`).
    let question = RwSignal::new(String::new());
    let pending = RwSignal::new(false);
    let convene_err = RwSignal::new(Option::<String>::None);
    // Topic count at submit time: the poll growing past it means the daemon
    // accepted and folded the new topic — re-enable the form then.
    let pending_baseline = RwSignal::new(0usize);

    Effect::new(move |_| {
        let count = topics.get().len();
        if pending.get_untracked() && count > pending_baseline.get_untracked() {
            pending.set(false);
        }
    });

    let on_convene = move |ev: web_sys::SubmitEvent| {
        ev.prevent_default();
        let q = question.get_untracked().trim().to_string();
        if q.is_empty() || pending.get_untracked() {
            return;
        }
        pending_baseline.set(topics.get_untracked().len());
        pending.set(true);
        convene_err.set(None);
        question.set(String::new());
        let base = url.get_untracked();
        spawn_local(async move {
            let outcome = post_convene(base, q).await;
            // Deck may have closed while the council deliberated — try_* only.
            let _ = pending.try_set(false);
            if let Err(msg) = outcome {
                let _ = convene_err.try_set(Some(msg));
            }
        });
    };

    view! {
        <div class="ocean-council-stage">
            <form class="ocean-council-convene" on:submit=on_convene>
                <input
                    class="ocean-council-convene__input"
                    type="text"
                    placeholder="convene a council — what should it decide?"
                    prop:value=move || question.get()
                    on:input=move |ev| question.set(event_target_value(&ev))
                />
                <button
                    class="ocean-council-convene__btn"
                    type="submit"
                    disabled=move || {
                        pending.get() || question.get().trim().is_empty()
                    }
                >
                    {move || if pending.get() { "convening…" } else { "convene" }}
                </button>
            </form>
            <Show when=move || convene_err.get().is_some()>
                <div class="ocean-council-convene__err">
                    {move || convene_err.get().unwrap_or_default()}
                </div>
            </Show>
            <Show
                when=move || !topics.get().is_empty()
                fallback=|| {
                    view! {
                        <div class="ocean-council-stage__empty">
                            <div class="ocean-council-stage__empty-title">
                                "no councils convened yet"
                            </div>
                            <div class="ocean-council-stage__empty-body">
                                "convene one above — members seat, marks land on the board, and quorum climbs toward a decision"
                            </div>
                        </div>
                    }
                }
            >
                <div class="ocean-council-stage__flow">
                    {move || {
                        topics
                            .get()
                            .into_iter()
                            .map(topic_card)
                            .collect_view()
                    }}
                </div>
            </Show>
        </div>
    }
}

/// One topic card. Built wholesale on each `topics` update (card counts are
/// tiny), so no `<For>` keying is needed — sidesteps the Leptos `<For>` stale
/// child trap where an unchanged key freezes mutating content.
fn topic_card(t: TopicSnapshot) -> impl IntoView {
    let converged = t.state == "converged";
    let topic_cls = format!("ocean-council-topic {}", state_class(&t.state));
    let federation = t.federation.to_uppercase();
    let state_label = t.state.clone();
    let title = if t.title.trim().is_empty() {
        "untitled topic".to_string()
    } else {
        t.title.clone()
    };
    let trigger_line = format!("{} · {} seated", t.trigger.replace('_', " "), t.members.len());

    let decision_label = t
        .decision
        .as_ref()
        .map(|id| proposal_label(&t.marks, id))
        .unwrap_or_else(|| "resolved".to_string());

    let quorum_pct = pct(t.distance_to_quorum);
    let quorum_fill = format!("width:{}", quorum_pct);
    let leader_label = t.leader.as_ref().map(|id| proposal_label(&t.marks, id));

    // Member chips — leadership roles (steward / firekeeper) get accent skin.
    let firekeeper = t.firekeeper.clone();
    let member_views = t
        .members
        .iter()
        .map(|m| {
            let is_lead = m.role == "steward"
                || m.role == "firekeeper"
                || firekeeper.as_deref() == Some(m.agent_id.as_str());
            let cls = if is_lead {
                "ocean-council-member is-lead"
            } else {
                "ocean-council-member"
            };
            let name = if m.label.trim().is_empty() {
                short8(&m.agent_id)
            } else {
                m.label.clone()
            };
            let model_view = (!m.model.is_empty()).then(|| {
                let model = m.model.clone();
                view! { <span class="ocean-council-member__model">{model}</span> }
            });
            view! {
                <span class=cls>
                    <span class="ocean-council-member__name">{name}</span>
                    {model_view}
                </span>
            }
        })
        .collect_view();

    // Tallies — ranked by weight, leader highlighted, bars normalized to the
    // top weight.
    let leader_id = t.leader.clone();
    let max_w = t
        .tallies
        .iter()
        .map(|x| x.net_weight)
        .fold(0.0_f32, f32::max)
        .max(0.0001);
    let mut tallies = t.tallies.clone();
    tallies.sort_by(|a, b| {
        b.net_weight
            .partial_cmp(&a.net_weight)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let marks_for_tally = t.marks.clone();
    let has_tallies = !tallies.is_empty();
    let tally_views = tallies
        .iter()
        .map(|ty| {
            let is_leader = leader_id.as_deref() == Some(ty.proposal.as_str());
            let cls = if is_leader {
                "ocean-council-tally is-leader"
            } else {
                "ocean-council-tally"
            };
            let name = proposal_label(&marks_for_tally, &ty.proposal);
            let w = (ty.net_weight / max_w * 100.0).clamp(0.0, 100.0);
            let fill = format!("width:{:.0}%", w);
            let weight = format!("{:.1}", ty.net_weight);
            view! {
                <div class=cls>
                    <span class="ocean-council-tally__name">{name}</span>
                    <span class="ocean-council-tally__track">
                        <span class="ocean-council-tally__fill" style=fill></span>
                    </span>
                    <span class="ocean-council-tally__weight">{weight}</span>
                </div>
            }
        })
        .collect_view();

    // Marks blackboard — left-rail column, kind-tagged, author-attributed.
    let members_for_marks = t.members.clone();
    let marks_for_target = t.marks.clone();
    let has_marks = !t.marks.is_empty();
    let mark_views = t
        .marks
        .iter()
        .map(|mk| {
            let kind = mk.kind.clone();
            let kind_cls = format!("ocean-council-mark__kind kind-{kind}");
            let author = member_label(&members_for_marks, &mk.author);
            let summary = trim_line(&mk.summary, 120);
            let target_view = mk.target.as_ref().map(|id| {
                let lbl = proposal_label(&marks_for_target, id);
                view! { <span class="ocean-council-mark__target">" → "{lbl}</span> }
            });
            view! {
                <div class="ocean-council-mark">
                    <div class="ocean-council-mark__head">
                        <span class=kind_cls>{kind}</span>
                        <span class="ocean-council-mark__author">{author}</span>
                    </div>
                    <div class="ocean-council-mark__summary">{summary}{target_view}</div>
                </div>
            }
        })
        .collect_view();

    let decision_view = converged.then(|| {
        view! {
            <div class="ocean-council-topic__decision">
                <span class="ocean-council-topic__decision-tag">"decided"</span>
                <span>{decision_label}</span>
            </div>
        }
    });

    let quorum_view = (!converged && has_tallies).then(|| {
        let leader_view = leader_label.map(|l| {
            view! { <div class="ocean-council-quorum__leader">"front-runner · "{l}</div> }
        });
        view! {
            <div class="ocean-council-quorum">
                <div class="ocean-council-quorum__head">
                    <span>"quorum"</span>
                    <span class="ocean-council-quorum__pct">{quorum_pct}</span>
                </div>
                <div class="ocean-council-quorum__bar">
                    <div class="ocean-council-quorum__fill" style=quorum_fill></div>
                </div>
                <div class="ocean-council-tallies">{tally_views}</div>
                {leader_view}
            </div>
        }
    });

    let marks_view = has_marks.then(|| {
        view! { <div class="ocean-council-marks">{mark_views}</div> }
    });

    view! {
        <div class=topic_cls>
            <div class="ocean-council-topic__head">
                <span class="ocean-council-topic__federation">{federation}</span>
                <span class="ocean-council-topic__state">
                    <span class="ocean-council-topic__dot"></span>
                    {state_label}
                </span>
            </div>
            <div class="ocean-council-topic__title">{title}</div>
            <div class="ocean-council-topic__trigger">{trigger_line}</div>
            {decision_view}
            {quorum_view}
            <div class="ocean-council-members">{member_views}</div>
            {marks_view}
        </div>
    }
}
