//! Progress and outcomes of Dash requests, reported through the root toast.
//!
//! Member commands, team policy sends, and consent rulings report here rather
//! than inside the card or control that issued them, so nothing in the Dash
//! resizes while a request is in flight. Each notice names the member or team
//! it concerns, the toast's live region announces it, and a request's outcome
//! replaces its progress. A request that disappears while still in flight
//! reports how it ended, so no progress message outlives its request.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use yew::prelude::*;

use crate::runtime::{ConsentRuling, MemberControlState};

use super::*;

/// The request a notice reports on.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Subject {
    Member(CanonicalUuid),
    Team(CanonicalUuid),
    Consent(CanonicalUuid),
}

/// Who a notice is attributed to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Owner {
    Member(CanonicalUuid),
    Team(CanonicalUuid),
}

#[derive(Clone, PartialEq, Eq)]
struct Notice {
    owner: Owner,
    text: String,
    /// Distinguishes two identical outcomes of successive requests.
    generation: u64,
    /// What a request still in flight reports if it disappears unsettled.
    ended: Option<String>,
}

/// Every request's current notice, and the member labels they name.
#[derive(Clone, Default, PartialEq)]
pub(super) struct Notices {
    notices: BTreeMap<Subject, Notice>,
    labels: BTreeMap<CanonicalUuid, String>,
}

impl Notices {
    pub(super) fn of(runtime: Option<&CanvasRuntime>, controls: &TeamControls) -> Self {
        let mut notices = Self::default();
        if let Some(runtime) = runtime {
            notices.read_runtime(runtime);
        }
        for (team, life, issue) in controls.requests() {
            let notice = match (life, issue) {
                (Some(life), _) => Notice {
                    owner: Owner::Team(team),
                    text: format!("Switching to {}…", life.label()),
                    generation: 0,
                    ended: Some(format!("Switched to {}", life.label())),
                },
                (None, Some(issue)) => Notice {
                    owner: Owner::Team(team),
                    text: issue.to_owned(),
                    generation: 0,
                    ended: None,
                },
                (None, None) => continue,
            };
            notices.notices.insert(Subject::Team(team), notice);
        }
        notices
    }

    fn read_runtime(&mut self, runtime: &CanvasRuntime) {
        for member in runtime
            .lanes()
            .into_iter()
            .flat_map(|lanes| &lanes.lanes)
            .flat_map(|team| &team.members)
        {
            self.labels.insert(
                member.lane,
                watchband::agent_message_label(&member.worktree, member.name.as_deref()).to_owned(),
            );
        }
        for (lane, state) in runtime.member_controls() {
            if let Some(notice) = member_notice(lane, state) {
                self.notices.insert(Subject::Member(lane), notice);
            }
        }
        for pending in runtime
            .consent_queue()
            .into_iter()
            .flat_map(|queue| &queue.pending)
        {
            if let Some(ruling) = runtime.consent_ruling(pending.request) {
                self.notices.insert(
                    Subject::Consent(pending.request),
                    consent_notice(pending, ruling),
                );
            }
        }
    }
}

fn member_notice(lane: CanonicalUuid, state: &MemberControlState) -> Option<Notice> {
    let owner = Owner::Member(lane);
    if let Some(request) = state.pending() {
        return Some(Notice {
            owner,
            text: request.command.progress().to_owned(),
            generation: request.serial,
            // The runtime settles every reservation it drops.
            ended: None,
        });
    }
    let settled = state.settled()?;
    Some(Notice {
        owner,
        text: settled
            .issue
            .clone()
            .unwrap_or_else(|| settled.request.command.done().to_owned()),
        generation: settled.request.serial,
        ended: None,
    })
}

fn consent_notice(pending: &wire::intent::ConsentPendingAnswer, ruling: &ConsentRuling) -> Notice {
    let operation = &pending.operation;
    let (text, ended) = match ruling {
        ConsentRuling::Sending(answer) => (
            format!("{} {operation}…", answer_verbs(*answer).0),
            Some(format!(
                "The decision on {operation} ended without an answer"
            )),
        ),
        ConsentRuling::Ruled(answer) => (format!("{} {operation}", answer_verbs(*answer).1), None),
        ConsentRuling::Refused(reason) => (format!("Could not decide {operation}: {reason}"), None),
    };
    Notice {
        owner: Owner::Member(pending.lane),
        text,
        generation: 0,
        ended,
    }
}

/// The in-flight and settled verbs for one ruling.
const fn answer_verbs(answer: wire::Answer) -> (&'static str, &'static str) {
    match answer {
        wire::Answer::Granted => ("Allowing", "Allowed"),
        wire::Answer::Denied => ("Denying", "Denied"),
        wire::Answer::Asked | wire::Answer::Queued => ("Answering", "Answered"),
    }
}

/// The notices last reported, and the label each member last carried so a
/// removed member's outcome still names it.
#[derive(Default)]
struct Reported {
    notices: BTreeMap<Subject, Notice>,
    labels: BTreeMap<CanonicalUuid, String>,
}

impl Reported {
    /// The toasts one change calls for, in order: progress first, so an
    /// outcome that lands in the same change stays visible.
    fn advance(&mut self, current: &Notices) -> Vec<(String, bool)> {
        self.labels.extend(
            current
                .labels
                .iter()
                .map(|(lane, label)| (*lane, label.clone())),
        );
        let mut toasts = Vec::new();
        for (subject, notice) in &current.notices {
            if self.notices.get(subject) != Some(notice) {
                toasts.push((
                    self.attribute(notice.owner, &notice.text),
                    notice.ended.is_some(),
                ));
            }
        }
        for (subject, notice) in &self.notices {
            if let Some(ended) = &notice.ended
                && !current.notices.contains_key(subject)
            {
                toasts.push((self.attribute(notice.owner, ended), false));
            }
        }
        toasts.sort_by_key(|(_, pending)| !pending);
        self.notices.clone_from(&current.notices);
        toasts
    }

    fn attribute(&self, owner: Owner, text: &str) -> String {
        match owner {
            Owner::Member(lane) => {
                let label = self
                    .labels
                    .get(&lane)
                    .cloned()
                    .unwrap_or_else(|| watchband::short(lane));
                format!("{label}: {text}")
            }
            Owner::Team(team) => format!("Team {}: {text}", watchband::short(team)),
        }
    }
}

#[derive(Properties, PartialEq)]
pub(super) struct DashFeedbackProps {
    pub(super) notices: Notices,
}

/// Toast each change to a Dash request's notice. The first render only
/// records what is already there, so remounting never replays old outcomes.
#[function_component(DashFeedback)]
pub(super) fn dash_feedback(props: &DashFeedbackProps) -> Html {
    let toaster = crate::browser::toast::use_toaster();
    let reported = use_mut_ref(|| None::<Reported>);
    use_effect_with(props.notices.clone(), move |notices| {
        report(&reported, &toaster, notices);
    });
    Html::default()
}

fn report(
    reported: &Rc<RefCell<Option<Reported>>>,
    toaster: &crate::browser::toast::Toaster,
    notices: &Notices,
) {
    let mut slot = reported.borrow_mut();
    let Some(reported) = slot.as_mut() else {
        let mut first = Reported::default();
        let _replayed = first.advance(notices);
        *slot = Some(first);
        return;
    };
    for (message, pending) in reported.advance(notices) {
        if pending {
            toaster.pending(message);
        } else {
            toaster.settled(message);
        }
    }
}
