//! CLI delivery law for an exhausted allocator drain.

use wire::Outcome;
use wire::intent::CommandAnswer;

/// Deliver one `task next` result, treating steered exhaustion as a clean drain.
pub(super) fn deliver(outcome: &Outcome<CommandAnswer>, form: super::Form, steered: bool) -> ! {
    if expected_empty(outcome, steered) {
        super::terminate_code(super::REPLY_EXIT);
    }
    super::deliver(outcome, form)
}

fn expected_empty(outcome: &Outcome<CommandAnswer>, steered: bool) -> bool {
    steered
        && matches!(
            outcome,
            Outcome::Refusal(refusal) if refusal.reason() == "board::allocator-empty"
        )
}

#[cfg(test)]
mod tests {
    use wire::{Fault, FaultClass, Outcome, Refusal};

    use super::expected_empty;

    fn refusal(reason: &str) -> Outcome<wire::intent::CommandAnswer> {
        Outcome::Refusal(Refusal::bare(wire::RefusalDiagnostic::new(
            reason.to_owned(),
            None,
            None,
            None,
        )))
    }

    fn fault() -> Outcome<wire::intent::CommandAnswer> {
        Outcome::Fault(Fault {
            class: FaultClass::Store,
            reason: "lineage::subscription-missing".to_owned(),
            detail: "allocator state is incomplete".to_owned(),
        })
    }

    #[test]
    fn steered_task_next_converts_only_expected_allocator_emptiness() {
        assert!(expected_empty(&refusal("board::allocator-empty"), true));
        assert!(!expected_empty(&refusal("board::allocator-empty"), false));
        assert!(!expected_empty(&refusal("claim::lane-unavailable"), true));
        assert!(!expected_empty(&fault(), true));
    }
}
