use clap::ArgMatches;
use journal::bundle::BundleVerification;
use wire::{Fault, FaultClass, Outcome, Refusal, Reply, ReplyPosition};

pub(crate) fn verify(matches: &ArgMatches) -> Outcome<BundleVerification> {
    let bundle = matches
        .get_one::<String>("BUNDLE")
        .expect("clap requires the restore bundle operand");
    match journal::bundle::verify_bundle(bundle) {
        Ok(body) => Outcome::Reply(Reply {
            position: ReplyPosition::Direct {},
            body,
        }),
        Err(error) if error.is_environment() => Outcome::Fault(Fault {
            class: FaultClass::Environment,
            reason: "restore::bundle-unavailable".to_owned(),
            detail: error.to_string(),
        }),
        Err(error) => Outcome::Refusal(Refusal {
            reason: "restore::bundle-refused".to_owned(),
            witness: None,
            expected: Some("one canonical current backup bundle".to_owned()),
            observed: Some(error.to_string()),
            remedy: "repair or replace the bundle before selecting it for restore".to_owned(),
        }),
    }
}
