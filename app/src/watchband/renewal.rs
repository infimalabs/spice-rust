//! Pure presentation for one member's durable agent-renewal state.

use wire::{LaneRenewalRecord, LaneRenewalState};

/// Complete presentation and next intent for one agent-renewal control.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRenewalControl {
    /// Stable machine state exposed to browser regressions.
    pub state: &'static str,
    /// Whether the control represents an outstanding renewal request.
    pub pressed: bool,
    /// Whether the current state can accept another operator action.
    pub disabled: bool,
    /// Boolean carried by the next typed renewal intent.
    pub next_requested: bool,
    /// Accessible name describing the exact next action or terminal state.
    pub label: String,
    /// Hover copy, including an actionable refusal when one exists.
    pub title: String,
    /// Primary visible action copy.
    pub heading: &'static str,
    /// Secondary visible state copy.
    pub detail: &'static str,
    /// Exact durable launch refusal, when this state carries one.
    pub refusal: Option<String>,
}

/// Derive one renewal control from durable lane state and current authority.
#[must_use]
pub fn agent_renewal_control(
    agent: &str,
    has_occupant: bool,
    can_send: bool,
    renewal: Option<&LaneRenewalRecord>,
) -> AgentRenewalControl {
    match renewal.map(|renewal| &renewal.state) {
        None => AgentRenewalControl {
            state: "none",
            pressed: false,
            disabled: !can_send || !has_occupant,
            next_requested: true,
            label: format!("Request agent renewal for {agent}"),
            title: format!("Start a fresh successor for {agent}"),
            heading: "Renew this agent",
            detail: "request successor",
            refusal: None,
        },
        Some(LaneRenewalState::Requested { .. }) => AgentRenewalControl {
            state: "requested",
            pressed: true,
            disabled: !can_send,
            next_requested: false,
            label: format!("Cancel requested renewal for {agent}"),
            title: format!("Cancel the pending successor request for {agent}"),
            heading: "Cancel renewal",
            detail: "successor requested",
            refusal: None,
        },
        Some(LaneRenewalState::HandoffPending { .. }) => AgentRenewalControl {
            state: "handoff-pending",
            pressed: true,
            disabled: true,
            next_requested: false,
            label: format!("Agent renewal handoff pending for {agent}"),
            title: format!("The predecessor stopped; {agent}'s successor is starting"),
            heading: "Renewing agent",
            detail: "handoff pending",
            refusal: None,
        },
        Some(LaneRenewalState::SuccessorStarted { .. }) => AgentRenewalControl {
            state: "successor-started",
            pressed: false,
            disabled: !can_send || !has_occupant,
            next_requested: true,
            label: format!("Request another agent renewal for {agent}"),
            title: format!("A fresh successor started for {agent}; request another renewal"),
            heading: "Renew again",
            detail: "successor started",
            refusal: None,
        },
        Some(LaneRenewalState::LaunchRefused { reason, .. }) => {
            let retry = format!("Retry agent renewal for {agent}: {reason}");
            AgentRenewalControl {
                state: "launch-refused",
                pressed: false,
                disabled: !can_send,
                next_requested: true,
                label: retry.clone(),
                title: retry,
                heading: "Retry renewal",
                detail: "launch refused",
                refusal: Some(reason.clone()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wire::RenewalIdentity;

    fn renewal(state: LaneRenewalState) -> LaneRenewalRecord {
        LaneRenewalRecord {
            lineage_id: "11111111-1111-4111-8111-111111111111"
                .parse()
                .expect("fixture lineage is canonical"),
            lane: "22222222-2222-4222-8222-222222222222"
                .parse()
                .expect("fixture lane is canonical"),
            revision: 7,
            state,
        }
    }

    fn identity(text: &str) -> RenewalIdentity {
        RenewalIdentity {
            occupant: text.parse().expect("fixture occupant is canonical"),
            session: format!("session-{text}"),
            thread: format!("thread-{text}"),
        }
    }

    #[test]
    fn exposes_request_cancel_handoff_and_successor_states() {
        let predecessor = identity("33333333-3333-4333-8333-333333333333");
        let successor = identity("44444444-4444-4444-8444-444444444444");
        let none = agent_renewal_control("Ada", true, true, None);
        assert_eq!(
            (none.state, none.pressed, none.disabled, none.next_requested),
            ("none", false, false, true),
        );

        let requested_record = renewal(LaneRenewalState::Requested {
            predecessor: predecessor.clone(),
            team_slot: 2,
        });
        let requested = agent_renewal_control("Ada", true, true, Some(&requested_record));
        assert_eq!(
            (
                requested.state,
                requested.pressed,
                requested.disabled,
                requested.next_requested,
                requested.heading,
            ),
            ("requested", true, false, false, "Cancel renewal"),
        );

        let handoff_record = renewal(LaneRenewalState::HandoffPending {
            predecessor: predecessor.clone(),
            team_slot: 2,
        });
        let handoff = agent_renewal_control("Ada", false, true, Some(&handoff_record));
        assert_eq!(
            (handoff.state, handoff.pressed, handoff.disabled),
            ("handoff-pending", true, true),
        );

        let succeeded_record = renewal(LaneRenewalState::SuccessorStarted {
            predecessor,
            successor,
            team_slot: 2,
        });
        let succeeded = agent_renewal_control("Ada", true, true, Some(&succeeded_record));
        assert_eq!(
            (
                succeeded.state,
                succeeded.pressed,
                succeeded.disabled,
                succeeded.heading,
            ),
            ("successor-started", false, false, "Renew again"),
        );
    }

    #[test]
    fn retains_exact_launch_refusal_and_authority() {
        let refused_record = renewal(LaneRenewalState::LaunchRefused {
            predecessor: identity("33333333-3333-4333-8333-333333333333"),
            team_slot: 2,
            reason: "configured executable is unavailable".to_owned(),
        });
        let refused = agent_renewal_control("Ada", false, true, Some(&refused_record));
        assert_eq!(refused.state, "launch-refused");
        assert!(!refused.disabled, "a terminal refusal remains retryable");
        assert_eq!(
            refused.refusal.as_deref(),
            Some("configured executable is unavailable"),
        );
        assert!(
            refused
                .label
                .contains("configured executable is unavailable")
        );

        let withheld = agent_renewal_control("Ada", true, false, None);
        assert!(
            withheld.disabled,
            "missing operator authority disables requests"
        );
    }
}
