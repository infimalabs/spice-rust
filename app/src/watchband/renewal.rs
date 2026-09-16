//! Pure presentation for one member's durable agent-renewal state.

use wire::{LaneRenewalRecord, LaneRenewalState};

/// The lane row seats nobody, so there is no predecessor to renew.
pub const NO_OCCUPANT_BLOCK: &str = "no occupant is seated";
/// The resident does not offer steering to this session.
pub const CANNOT_SEND_BLOCK: &str = "steering is not offered to this session";
/// The predecessor already stopped and its successor has not seated yet.
pub const HANDOFF_BLOCK: &str = "handoff in progress";

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
    /// The condition withholding operator action, exactly when disabled.
    pub blocked: Option<&'static str>,
}

/// Derive one renewal control from durable lane state and current authority.
///
/// A disabled control always names its condition: the operator must be able to
/// tell an empty or stale seat from a busy one without a working hover.
#[must_use]
pub fn agent_renewal_control(
    agent: &str,
    has_occupant: bool,
    can_send: bool,
    renewal: Option<&LaneRenewalRecord>,
) -> AgentRenewalControl {
    let state = renewal.map(|renewal| &renewal.state);
    let blocked = match state {
        None | Some(LaneRenewalState::SuccessorStarted { .. }) => {
            request_block(has_occupant, can_send)
        }
        Some(LaneRenewalState::HandoffPending { .. }) => Some(HANDOFF_BLOCK),
        Some(LaneRenewalState::Requested { .. } | LaneRenewalState::LaunchRefused { .. }) => {
            (!can_send).then_some(CANNOT_SEND_BLOCK)
        }
    };
    let mut control = presentation(agent, state);
    if let Some(block) = blocked {
        control.disabled = true;
        control.blocked = Some(block);
        control.label = format!("{} (unavailable: {block})", control.label);
        control.title = format!("{}. Unavailable: {block}", control.title);
    }
    control
}

/// Name whichever condition withholds a fresh request, seat first.
const fn request_block(has_occupant: bool, can_send: bool) -> Option<&'static str> {
    if !has_occupant {
        Some(NO_OCCUPANT_BLOCK)
    } else if !can_send {
        Some(CANNOT_SEND_BLOCK)
    } else {
        None
    }
}

/// The enabled presentation of one durable renewal state.
fn presentation(agent: &str, state: Option<&LaneRenewalState>) -> AgentRenewalControl {
    match state {
        None => AgentRenewalControl {
            state: "none",
            pressed: false,
            disabled: false,
            next_requested: true,
            label: format!("Request agent renewal for {agent}"),
            title: format!("Start a fresh successor for {agent}"),
            heading: "Renew this agent",
            detail: "request successor",
            refusal: None,
            blocked: None,
        },
        Some(LaneRenewalState::Requested { .. }) => AgentRenewalControl {
            state: "requested",
            pressed: true,
            disabled: false,
            next_requested: false,
            label: format!("Cancel requested renewal for {agent}"),
            title: format!("Cancel the pending successor request for {agent}"),
            heading: "Cancel renewal",
            detail: "successor requested",
            refusal: None,
            blocked: None,
        },
        Some(LaneRenewalState::HandoffPending { .. }) => AgentRenewalControl {
            state: "handoff-pending",
            pressed: true,
            disabled: false,
            next_requested: false,
            label: format!("Agent renewal handoff pending for {agent}"),
            title: format!("The predecessor stopped; {agent}'s successor is starting"),
            heading: "Renewing agent",
            detail: "handoff pending",
            refusal: None,
            blocked: None,
        },
        Some(LaneRenewalState::SuccessorStarted { .. }) => AgentRenewalControl {
            state: "successor-started",
            pressed: false,
            disabled: false,
            next_requested: true,
            label: format!("Request another agent renewal for {agent}"),
            title: format!("A fresh successor started for {agent}; request another renewal"),
            heading: "Renew again",
            detail: "successor started",
            refusal: None,
            blocked: None,
        },
        Some(LaneRenewalState::LaunchRefused { reason, .. }) => {
            let retry = format!("Retry agent renewal for {agent}: {reason}");
            AgentRenewalControl {
                state: "launch-refused",
                pressed: false,
                disabled: false,
                next_requested: true,
                label: retry.clone(),
                title: retry,
                heading: "Retry renewal",
                detail: "launch refused",
                refusal: Some(reason.clone()),
                blocked: None,
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
            org_slot: 2,
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
            org_slot: 2,
        });
        let handoff = agent_renewal_control("Ada", false, true, Some(&handoff_record));
        assert_eq!(
            (handoff.state, handoff.pressed, handoff.disabled),
            ("handoff-pending", true, true),
        );

        let succeeded_record = renewal(LaneRenewalState::SuccessorStarted {
            predecessor,
            successor,
            org_slot: 2,
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
            org_slot: 2,
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
        assert_eq!(withheld.blocked, Some(CANNOT_SEND_BLOCK));
    }

    #[test]
    fn names_the_condition_that_disables_each_control() {
        let vacant = agent_renewal_control("Ada", false, true, None);
        assert_eq!(vacant.blocked, Some(NO_OCCUPANT_BLOCK));
        assert!(vacant.disabled);
        assert_eq!(
            vacant.label,
            "Request agent renewal for Ada (unavailable: no occupant is seated)",
        );
        assert_eq!(
            vacant.title,
            "Start a fresh successor for Ada. Unavailable: no occupant is seated",
        );

        let both = agent_renewal_control("Ada", false, false, None);
        assert_eq!(
            both.blocked,
            Some(NO_OCCUPANT_BLOCK),
            "the seat is named first"
        );

        let enabled = agent_renewal_control("Ada", true, true, None);
        assert_eq!(enabled.blocked, None);
        assert_eq!(enabled.label, "Request agent renewal for Ada");
        assert_eq!(enabled.title, "Start a fresh successor for Ada");

        let predecessor = identity("33333333-3333-4333-8333-333333333333");
        let handoff_record = renewal(LaneRenewalState::HandoffPending {
            predecessor: predecessor.clone(),
            org_slot: 2,
        });
        let handoff = agent_renewal_control("Ada", true, true, Some(&handoff_record));
        assert_eq!(handoff.blocked, Some(HANDOFF_BLOCK));
        assert!(
            handoff
                .title
                .ends_with(". Unavailable: handoff in progress")
        );

        let requested_record = renewal(LaneRenewalState::Requested {
            predecessor: predecessor.clone(),
            org_slot: 2,
        });
        let cancel = agent_renewal_control("Ada", false, false, Some(&requested_record));
        assert_eq!(
            cancel.blocked,
            Some(CANNOT_SEND_BLOCK),
            "cancelling needs authority, not a seated occupant"
        );
        assert!(cancel.disabled);

        let refused_record = renewal(LaneRenewalState::LaunchRefused {
            predecessor,
            org_slot: 2,
            reason: "configured executable is unavailable".to_owned(),
        });
        let refused = agent_renewal_control("Ada", false, false, Some(&refused_record));
        assert_eq!(refused.blocked, Some(CANNOT_SEND_BLOCK));
        assert_eq!(
            refused.refusal.as_deref(),
            Some("configured executable is unavailable"),
            "the durable launch refusal survives beside the block"
        );
    }

    #[test]
    fn disabled_exactly_when_a_block_is_named() {
        let predecessor = identity("33333333-3333-4333-8333-333333333333");
        let successor = identity("44444444-4444-4444-8444-444444444444");
        let records = [
            None,
            Some(renewal(LaneRenewalState::Requested {
                predecessor: predecessor.clone(),
                org_slot: 1,
            })),
            Some(renewal(LaneRenewalState::HandoffPending {
                predecessor: predecessor.clone(),
                org_slot: 1,
            })),
            Some(renewal(LaneRenewalState::SuccessorStarted {
                predecessor: predecessor.clone(),
                successor,
                org_slot: 1,
            })),
            Some(renewal(LaneRenewalState::LaunchRefused {
                predecessor,
                org_slot: 1,
                reason: "refused".to_owned(),
            })),
        ];
        for record in &records {
            for (has_occupant, can_send) in
                [(true, true), (true, false), (false, true), (false, false)]
            {
                let control = agent_renewal_control("Ada", has_occupant, can_send, record.as_ref());
                assert_eq!(
                    control.disabled,
                    control.blocked.is_some(),
                    "{} with occupant={has_occupant} send={can_send}",
                    control.state,
                );
            }
        }
    }
}
