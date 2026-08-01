use wire::intent::CommandAnswer;
use wire::{Outcome, Refusal};

pub(super) fn steering_refusal(
    reason: &str,
    observed: Option<String>,
    remedy: &str,
) -> Outcome<CommandAnswer> {
    Outcome::Refusal(Refusal {
        reason: reason.to_owned(),
        witness: Some("steering-inbox".to_owned()),
        expected: Some("one live directive in the authenticated lane".to_owned()),
        observed,
        remedy: remedy.to_owned(),
    })
}
