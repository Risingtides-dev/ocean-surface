//! Pure Voice Planner contract: bounded proposal data, deterministic handoff
//! rendering, and a reducer that makes the human-confirmed mutation boundary
//! explicit. This module owns no HTTP, microphone, session, or daemon state.

use serde::{Deserialize, Serialize};

pub const TITLE_MAX_CHARS: usize = 120;
pub const FIELD_MAX_CHARS: usize = 2_000;
pub const LIST_MAX_ITEMS: usize = 32;
pub const MARKDOWN_MAX_BYTES: usize = 24 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannerContext {
    pub project_id: String,
    pub project_name: String,
    pub workspace_root: String,
}

impl PlannerContext {
    pub fn validate(&self) -> Result<(), String> {
        if self.project_id.trim().is_empty()
            || self.project_name.trim().is_empty()
            || self.workspace_root.trim().is_empty()
        {
            return Err("project and workspace are required".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoicePlannerBrief {
    pub title: String,
    pub problem: String,
    pub users: Vec<String>,
    pub goals: Vec<String>,
    pub non_goals: Vec<String>,
    pub requirements: Vec<String>,
    pub acceptance_criteria: Vec<String>,
    pub constraints: Vec<String>,
    pub open_questions: Vec<String>,
}

impl VoicePlannerBrief {
    pub fn from_tool_arguments(raw: &str) -> Result<Self, String> {
        let brief: Self =
            serde_json::from_str(raw).map_err(|error| format!("invalid proposal JSON: {error}"))?;
        brief.validate()?;
        Ok(brief)
    }

    pub fn validate(&self) -> Result<(), String> {
        validate_required("title", &self.title, TITLE_MAX_CHARS)?;
        validate_required("problem", &self.problem, FIELD_MAX_CHARS)?;
        for (name, values) in self.lists() {
            if values.len() > LIST_MAX_ITEMS {
                return Err(format!("{name} exceeds {LIST_MAX_ITEMS} items"));
            }
            for value in values {
                if value.chars().count() > FIELD_MAX_CHARS {
                    return Err(format!("{name} item exceeds {FIELD_MAX_CHARS} characters"));
                }
            }
        }
        Ok(())
    }

    fn lists(&self) -> [(&'static str, &[String]); 7] {
        [
            ("users", &self.users),
            ("goals", &self.goals),
            ("non_goals", &self.non_goals),
            ("requirements", &self.requirements),
            ("acceptance_criteria", &self.acceptance_criteria),
            ("constraints", &self.constraints),
            ("open_questions", &self.open_questions),
        ]
    }

    /// Render the confirmed wire payload. Values are escaped as Markdown text,
    /// never treated as model-authored Markdown or HTML.
    pub fn render_markdown(&self, context: &PlannerContext) -> Result<String, String> {
        self.validate()?;
        context.validate()?;
        let mut out = String::new();
        push_heading_value(&mut out, 1, &self.title);
        push_heading_value(&mut out, 2, "Project");
        out.push_str(&escape_markdown_text(context.project_name.trim()));
        out.push_str("\n\n");
        push_heading_value(&mut out, 2, "Workspace");
        out.push_str(&escape_markdown_text(context.workspace_root.trim()));
        out.push_str("\n\n");
        push_text_section(&mut out, "Problem", &self.problem);
        push_list_section(&mut out, "Users", &self.users);
        push_list_section(&mut out, "Goals", &self.goals);
        push_list_section(&mut out, "Non-goals", &self.non_goals);
        push_list_section(&mut out, "Requirements", &self.requirements);
        push_list_section(&mut out, "Acceptance criteria", &self.acceptance_criteria);
        push_list_section(&mut out, "Constraints", &self.constraints);
        push_list_section(&mut out, "Open questions", &self.open_questions);
        out.push_str("Use this confirmed PRD as the planning/implementation brief. Respect its non-goals, constraints, and acceptance criteria.");
        if out.len() > MARKDOWN_MAX_BYTES {
            return Err(format!(
                "confirmed Markdown exceeds {MARKDOWN_MAX_BYTES} UTF-8 bytes"
            ));
        }
        Ok(out)
    }
}

fn validate_required(name: &str, value: &str, max: usize) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("{name} must not be blank"));
    }
    if value.chars().count() > max {
        return Err(format!("{name} exceeds {max} characters"));
    }
    Ok(())
}

fn escape_markdown_text(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        if matches!(
            ch,
            '\\' | '`' | '*' | '_' | '{' | '}' | '[' | ']' | '<' | '>' | '#' | '|' | '~'
        ) {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

fn push_heading_value(out: &mut String, level: usize, value: &str) {
    out.push_str(&"#".repeat(level));
    out.push(' ');
    out.push_str(&escape_markdown_text(value.trim()));
    out.push_str("\n\n");
}

fn push_text_section(out: &mut String, heading: &str, value: &str) {
    push_heading_value(out, 2, heading);
    out.push_str(&escape_markdown_text(value.trim()));
    out.push_str("\n\n");
}

fn push_list_section(out: &mut String, heading: &str, values: &[String]) {
    push_heading_value(out, 2, heading);
    for value in values {
        out.push_str("- ");
        out.push_str(&escape_markdown_text(value.trim()));
        out.push('\n');
    }
    if !values.is_empty() {
        out.push('\n');
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlannerAction {
    CreateDraft,
    CreateAndStart,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlannerState {
    Idle,
    Selecting,
    Gathering {
        generation: u64,
        context: PlannerContext,
        proposal: Option<VoicePlannerBrief>,
    },
    Confirming {
        generation: u64,
        context: PlannerContext,
        proposal: VoicePlannerBrief,
        markdown: String,
        action: PlannerAction,
        session_id: Option<String>,
    },
    PartialFailure {
        generation: u64,
        context: PlannerContext,
        proposal: VoicePlannerBrief,
        markdown: String,
        action: PlannerAction,
        session_id: String,
        error: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlannerEvent {
    Open,
    Start {
        generation: u64,
        context: PlannerContext,
    },
    Proposal {
        generation: u64,
        proposal: VoicePlannerBrief,
    },
    Confirm(PlannerAction),
    SessionCreated {
        generation: u64,
        session_id: String,
    },
    StepSucceeded {
        generation: u64,
        session_id: String,
    },
    StepFailed {
        generation: u64,
        session_id: String,
        error: String,
    },
    Retry,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlannerEffect {
    ConnectRealtime {
        generation: u64,
        context: PlannerContext,
    },
    StopRealtime,
    CreateSession {
        generation: u64,
        context: PlannerContext,
    },
    AdoptSession {
        generation: u64,
        session_id: String,
        title: String,
    },
    AppendHandoff {
        generation: u64,
        session_id: String,
        markdown: String,
    },
    SubmitTurn {
        generation: u64,
        session_id: String,
        context: PlannerContext,
        markdown: String,
    },
}

pub fn reduce(state: &mut PlannerState, event: PlannerEvent) -> Result<Vec<PlannerEffect>, String> {
    match event {
        PlannerEvent::Open if matches!(state, PlannerState::Idle) => {
            *state = PlannerState::Selecting;
            Ok(vec![])
        }
        PlannerEvent::Start {
            generation,
            context,
        } if matches!(state, PlannerState::Selecting) => {
            context.validate()?;
            *state = PlannerState::Gathering {
                generation,
                context: context.clone(),
                proposal: None,
            };
            Ok(vec![PlannerEffect::ConnectRealtime {
                generation,
                context,
            }])
        }
        PlannerEvent::Proposal {
            generation,
            proposal,
        } => {
            proposal.validate()?;
            if let PlannerState::Gathering {
                generation: current,
                proposal: current_proposal,
                ..
            } = state
            {
                if *current == generation {
                    *current_proposal = Some(proposal);
                }
            }
            Ok(vec![])
        }
        PlannerEvent::Confirm(action) => {
            let PlannerState::Gathering {
                generation,
                context,
                proposal: Some(proposal),
            } = state
            else {
                return Ok(vec![]);
            };
            let markdown = proposal.render_markdown(context)?;
            let effect = PlannerEffect::CreateSession {
                generation: *generation,
                context: context.clone(),
            };
            *state = PlannerState::Confirming {
                generation: *generation,
                context: context.clone(),
                proposal: proposal.clone(),
                markdown,
                action,
                session_id: None,
            };
            Ok(vec![PlannerEffect::StopRealtime, effect])
        }
        PlannerEvent::SessionCreated {
            generation,
            session_id,
        } => {
            let PlannerState::Confirming {
                generation: current,
                context,
                proposal,
                markdown,
                action,
                session_id: current_session,
            } = state
            else {
                return Ok(vec![]);
            };
            if *current != generation || current_session.is_some() || session_id.trim().is_empty() {
                return Ok(vec![]);
            }
            *current_session = Some(session_id.clone());
            let title = proposal.title.trim().to_string();
            let second = match action {
                PlannerAction::CreateDraft => PlannerEffect::AppendHandoff {
                    generation,
                    session_id: session_id.clone(),
                    markdown: markdown.clone(),
                },
                PlannerAction::CreateAndStart => PlannerEffect::SubmitTurn {
                    generation,
                    session_id: session_id.clone(),
                    context: context.clone(),
                    markdown: markdown.clone(),
                },
            };
            Ok(vec![
                PlannerEffect::AdoptSession {
                    generation,
                    session_id,
                    title,
                },
                second,
            ])
        }
        PlannerEvent::StepFailed {
            generation,
            session_id,
            error,
        } => {
            let PlannerState::Confirming {
                generation: current,
                context,
                proposal,
                markdown,
                action,
                session_id: Some(current_session),
            } = state
            else {
                return Ok(vec![]);
            };
            if *current != generation || *current_session != session_id {
                return Ok(vec![]);
            }
            *state = PlannerState::PartialFailure {
                generation,
                context: context.clone(),
                proposal: proposal.clone(),
                markdown: markdown.clone(),
                action: *action,
                session_id,
                error,
            };
            Ok(vec![])
        }
        PlannerEvent::Retry => {
            let PlannerState::PartialFailure {
                generation,
                context,
                proposal,
                markdown,
                action,
                session_id,
                ..
            } = state
            else {
                return Ok(vec![]);
            };
            let effect = match action {
                PlannerAction::CreateDraft => PlannerEffect::AppendHandoff {
                    generation: *generation,
                    session_id: session_id.clone(),
                    markdown: markdown.clone(),
                },
                PlannerAction::CreateAndStart => PlannerEffect::SubmitTurn {
                    generation: *generation,
                    session_id: session_id.clone(),
                    context: context.clone(),
                    markdown: markdown.clone(),
                },
            };
            *state = PlannerState::Confirming {
                generation: *generation,
                context: context.clone(),
                proposal: proposal.clone(),
                markdown: markdown.clone(),
                action: *action,
                session_id: Some(session_id.clone()),
            };
            Ok(vec![effect])
        }
        PlannerEvent::StepSucceeded {
            generation,
            session_id,
        } => {
            let matches = matches!(
                state,
                PlannerState::Confirming {
                    generation: current,
                    session_id: Some(current_session),
                    ..
                } if *current == generation && *current_session == session_id
            );
            if matches {
                *state = PlannerState::Idle;
            }
            Ok(vec![])
        }
        PlannerEvent::Cancel => {
            let stop = matches!(
                state,
                PlannerState::Gathering { .. } | PlannerState::Confirming { .. }
            );
            *state = PlannerState::Idle;
            Ok(if stop {
                vec![PlannerEffect::StopRealtime]
            } else {
                vec![]
            })
        }
        _ => Ok(vec![]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> PlannerContext {
        PlannerContext {
            project_id: "00000000-0000-0000-0000-000000000001".into(),
            project_name: "Ocean [Surface]".into(),
            workspace_root: "/tmp/ocean_surface".into(),
        }
    }

    fn brief() -> VoicePlannerBrief {
        VoicePlannerBrief {
            title: "Ship *planner*".into(),
            problem: "Users need <safe> planning.".into(),
            users: vec!["Operators".into()],
            goals: vec!["Review first".into()],
            non_goals: vec![],
            requirements: vec!["Exact click".into()],
            acceptance_criteria: vec!["One turn".into()],
            constraints: vec!["No auto-create".into()],
            open_questions: vec![],
        }
    }

    #[test]
    fn strict_parse_rejects_unknown_missing_blank_and_overlong_fields() {
        let mut value = serde_json::to_value(brief()).unwrap();
        value["surprise"] = serde_json::json!(true);
        assert!(VoicePlannerBrief::from_tool_arguments(&value.to_string()).is_err());
        let mut blank = brief();
        blank.title = "  ".into();
        assert!(blank.validate().is_err());
        let mut long = brief();
        long.goals = vec!["x".repeat(FIELD_MAX_CHARS + 1)];
        assert!(long.validate().is_err());
        let mut many = brief();
        many.users = vec!["x".into(); LIST_MAX_ITEMS + 1];
        assert!(many.validate().is_err());
    }

    #[test]
    fn markdown_is_stable_escaped_and_keeps_empty_heading_order() {
        let out = brief().render_markdown(&context()).unwrap();
        assert!(out.starts_with("# Ship \\*planner\\*\n\n## Project\n\nOcean \\[Surface\\]"));
        assert!(out.contains("Users need \\<safe\\> planning."));
        let headings = [
            "## Project",
            "## Workspace",
            "## Problem",
            "## Users",
            "## Goals",
            "## Non-goals",
            "## Requirements",
            "## Acceptance criteria",
            "## Constraints",
            "## Open questions",
        ];
        let mut last = 0;
        for heading in headings {
            let pos = out.find(heading).unwrap();
            assert!(pos >= last, "heading order changed at {heading}");
            last = pos;
        }
        assert!(out.contains("## Non-goals\n\n## Requirements"));
        assert!(out.ends_with("Use this confirmed PRD as the planning/implementation brief. Respect its non-goals, constraints, and acceptance criteria."));
    }

    #[test]
    fn final_utf8_markdown_cap_is_enforced_without_truncation() {
        let mut huge = brief();
        huge.users = vec!["é".repeat(FIELD_MAX_CHARS); LIST_MAX_ITEMS];
        assert!(huge.render_markdown(&context()).is_err());
    }

    #[test]
    fn proposal_has_no_effect_context_is_frozen_and_double_confirm_is_suppressed() {
        let mut state = PlannerState::Selecting;
        let frozen = context();
        assert_eq!(
            reduce(
                &mut state,
                PlannerEvent::Start {
                    generation: 7,
                    context: frozen.clone(),
                }
            )
            .unwrap(),
            vec![PlannerEffect::ConnectRealtime {
                generation: 7,
                context: frozen.clone()
            }]
        );
        assert!(reduce(
            &mut state,
            PlannerEvent::Proposal {
                generation: 7,
                proposal: brief()
            }
        )
        .unwrap()
        .is_empty());
        let first = reduce(
            &mut state,
            PlannerEvent::Confirm(PlannerAction::CreateAndStart),
        )
        .unwrap();
        assert!(
            matches!(first.as_slice(), [PlannerEffect::StopRealtime, PlannerEffect::CreateSession { context, .. }] if context == &frozen)
        );
        assert!(reduce(
            &mut state,
            PlannerEvent::Confirm(PlannerAction::CreateAndStart)
        )
        .unwrap()
        .is_empty());
    }

    #[test]
    fn created_session_is_adopted_then_exact_action_and_partial_retry_reuses_it() {
        for action in [PlannerAction::CreateDraft, PlannerAction::CreateAndStart] {
            let mut state = PlannerState::Gathering {
                generation: 2,
                context: context(),
                proposal: Some(brief()),
            };
            reduce(&mut state, PlannerEvent::Confirm(action)).unwrap();
            let effects = reduce(
                &mut state,
                PlannerEvent::SessionCreated {
                    generation: 2,
                    session_id: "s1".into(),
                },
            )
            .unwrap();
            assert!(
                matches!(effects.first(), Some(PlannerEffect::AdoptSession { session_id, .. }) if session_id == "s1")
            );
            match action {
                PlannerAction::CreateDraft => assert!(matches!(
                    effects.get(1),
                    Some(PlannerEffect::AppendHandoff { .. })
                )),
                PlannerAction::CreateAndStart => assert!(matches!(
                    effects.get(1),
                    Some(PlannerEffect::SubmitTurn { .. })
                )),
            }
            reduce(
                &mut state,
                PlannerEvent::StepFailed {
                    generation: 2,
                    session_id: "s1".into(),
                    error: "network".into(),
                },
            )
            .unwrap();
            let retry = reduce(&mut state, PlannerEvent::Retry).unwrap();
            assert_eq!(retry.len(), 1);
            assert!(!matches!(retry[0], PlannerEffect::CreateSession { .. }));
        }
    }

    #[test]
    fn stale_generation_and_wrong_session_results_do_nothing() {
        let mut state = PlannerState::Gathering {
            generation: 9,
            context: context(),
            proposal: None,
        };
        reduce(
            &mut state,
            PlannerEvent::Proposal {
                generation: 8,
                proposal: brief(),
            },
        )
        .unwrap();
        assert!(matches!(
            state,
            PlannerState::Gathering { proposal: None, .. }
        ));
    }
}
